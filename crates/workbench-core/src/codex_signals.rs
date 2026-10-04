//! Opt-in, observational Codex callbacks. Never read transcripts or send input.
use std::{
    collections::{HashSet, hash_map::DefaultHasher},
    fs::{self, OpenOptions},
    hash::{Hash, Hasher},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{process::Command, time::timeout};

use crate::{AgentStatus, Engine, WorkItemState, tmux::TmuxClient, work_items::StateLock};

pub(crate) const MAX_PAYLOAD: usize = 128 * 1024;
const MAX_RECORDS: usize = 128;
const MAX_STORE: u64 = 4 * 1024 * 1024;
// A lost callback must not survive indefinitely. This is NOT an inactivity timer:
// quiet turns remain running while their bound process lives, for up to a day.
const MAX_AGE: u64 = 24 * 60 * 60 * 1_000_000;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct Binding {
    socket: String,
    server: String,
    pane: String,
    pane_pid: u32,
    agent_pid: u32,
    agent_start: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
enum Phase {
    Idle,
    Running,
    Complete,
    Interrupted,
    Ended,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Question {
    text: String,
    options: Vec<(String, String)>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Pending {
    call: Option<String>,
    questions: Vec<Question>,
    confirmed: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Record {
    binding: Binding,
    session: String,
    turn: Option<String>,
    retired: Vec<String>,
    retired_sessions: Vec<String>,
    phase: Phase,
    pending: Option<Pending>,
    stamp: u64,
}

#[derive(Clone, Debug)]
struct Event {
    session: String,
    turn: Option<String>,
    name: String,
    call: Option<String>,
    questions: Vec<Question>,
}

fn identifier(value: &Value, key: &str) -> Option<String> {
    let text = value.get(key)?.as_str()?;
    (!text.is_empty() && text.len() <= 128 && !text.chars().any(char::is_control))
        .then(|| text.to_owned())
}

fn parse(payload: &str, notification: bool) -> Option<Event> {
    if payload.len() > MAX_PAYLOAD {
        return None;
    }
    let value: Value = serde_json::from_str(payload).ok()?;
    if value.get("agent_id").is_some_and(|v| !v.is_null()) {
        return None;
    }
    let (session, turn, name) = if notification {
        if value.get("type")?.as_str()? != "agent-turn-complete" {
            return None;
        }
        (
            identifier(&value, "thread-id")?,
            Some(identifier(&value, "turn-id")?),
            "Complete".to_owned(),
        )
    } else {
        (
            identifier(&value, "session_id")?,
            identifier(&value, "turn_id"),
            value.get("hook_event_name")?.as_str()?.to_owned(),
        )
    };
    if !matches!(
        name.as_str(),
        "SessionStart"
            | "SessionEnd"
            | "UserPromptSubmit"
            | "PreToolUse"
            | "PostToolUse"
            | "PermissionRequest"
            | "Stop"
            | "Interrupt"
            | "Complete"
    ) || (!matches!(name.as_str(), "SessionStart" | "SessionEnd") && turn.is_none())
    {
        return None;
    }
    let mut questions = Vec::new();
    if name == "PreToolUse"
        && value.get("tool_name").and_then(Value::as_str) == Some("request_user_input")
    {
        questions = parse_questions(&value).unwrap_or_default();
    }
    Some(Event {
        session,
        turn,
        name,
        call: identifier(&value, "tool_use_id"),
        questions,
    })
}

fn parse_questions(value: &Value) -> Option<Vec<Question>> {
    let items = value.get("tool_input")?.get("questions")?.as_array()?;
    if items.is_empty() || items.len() > 4 {
        return None;
    }
    let mut ids = HashSet::new();
    let mut result = Vec::new();
    for item in items {
        if !ids.insert(identifier(item, "id")?) {
            return None;
        }
        let text = bounded_text(item.get("question")?, 2048)?;
        let options = item.get("options")?.as_array()?;
        if options.is_empty() || options.len() > 8 {
            return None;
        }
        let options = options
            .iter()
            .map(|option| {
                Some((
                    bounded_text(option.get("label")?, 256)?,
                    bounded_text(option.get("description")?, 512)?,
                ))
            })
            .collect::<Option<Vec<_>>>()?;
        result.push(Question { text, options });
    }
    Some(result)
}

fn bounded_text(value: &Value, limit: usize) -> Option<String> {
    let text = value.as_str()?;
    (!text.is_empty()
        && text.len() <= limit
        && !text
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t'))
    .then(|| text.to_owned())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn transition(records: &mut Vec<Record>, binding: Binding, event: Event, stamp: u64) {
    records.retain(|record| stamp.saturating_sub(record.stamp) <= MAX_AGE);
    let existing = records.iter().position(|r| r.binding == binding);
    if existing.is_none() && event.name != "SessionStart" {
        // No heuristic workspace matching and no orphan completion adoption.
        return;
    }
    let index = existing.unwrap_or_else(|| {
        if records.len() == MAX_RECORDS {
            records.remove(0);
        }
        records.push(Record {
            binding,
            session: event.session.clone(),
            turn: None,
            retired: Vec::new(),
            retired_sessions: Vec::new(),
            phase: Phase::Idle,
            pending: None,
            stamp,
        });
        records.len() - 1
    });
    let record = &mut records[index];
    if stamp < record.stamp {
        return;
    }
    if event.name == "SessionStart" {
        if record.session != event.session {
            if record.retired_sessions.contains(&event.session) {
                return;
            }
            record.retired_sessions.push(record.session.clone());
            if record.retired_sessions.len() > 32 {
                record.retired_sessions.remove(0);
            }
            record.session = event.session;
            record.turn = None;
            record.retired.clear();
            record.phase = Phase::Idle;
            record.pending = None;
            record.stamp = stamp;
        } else if record.phase == Phase::Ended {
            record.phase = Phase::Idle;
            record.stamp = stamp;
        }
        // Compact/resume startup for the same session must not reset a turn.
        return;
    }
    if record.session != event.session || record.phase == Phase::Ended {
        return;
    }
    if event.name == "SessionEnd" {
        record.phase = Phase::Ended;
        record.pending = None;
    } else if event.name == "UserPromptSubmit" {
        if record
            .retired
            .contains(event.turn.as_ref().expect("validated turn"))
        {
            return;
        }
        if record.turn == event.turn {
            return;
        }
        if let Some(old) = record.turn.take() {
            record.retired.push(old);
            if record.retired.len() > 32 {
                record.retired.remove(0);
            }
        }
        record.turn = event.turn;
        record.phase = Phase::Running;
        record.pending = None;
    } else {
        if record.turn != event.turn || record.phase != Phase::Running {
            return;
        }
        match event.name.as_str() {
            "Complete" => {
                record.phase = Phase::Complete;
                record.pending = None;
            }
            "Interrupt" => {
                record.phase = Phase::Interrupted;
                record.pending = None;
            }
            "PermissionRequest" => {
                record.pending = Some(Pending {
                    call: None,
                    questions: Vec::new(),
                    confirmed: false,
                });
            }
            "PreToolUse" => {
                if !event.questions.is_empty() {
                    record.pending = Some(Pending {
                        call: event.call,
                        questions: event.questions,
                        confirmed: false,
                    });
                }
            }
            "PostToolUse" => {
                if record
                    .pending
                    .as_ref()
                    .is_some_and(|p| p.call.is_some() && p.call == event.call)
                {
                    record.pending = None;
                }
            }
            "Stop" => {
                record.pending = None;
            } // Stop can continue; never complete here.
            _ => {}
        }
    }
    record.stamp = stamp;
}

fn path(state: &Path) -> PathBuf {
    let mut name = state.as_os_str().to_owned();
    name.push(".codex-signals.json");
    name.into()
}

fn load(path: &Path) -> io::Result<Vec<Record>> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut bytes = Vec::new();
    file.take(MAX_STORE + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_STORE {
        return Err(io::Error::other("Signal store too large."));
    }
    let records: Vec<Record> = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if records.len() > MAX_RECORDS {
        return Err(io::Error::other("Too many signal records."));
    }
    Ok(records)
}

fn update(state: &Path, edit: impl FnOnce(&mut Vec<Record>)) -> io::Result<()> {
    let path = path(state);
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("Missing state directory."))?;
    fs::create_dir_all(parent)?;
    let mut lock_path = path.as_os_str().to_owned();
    lock_path.push(".lock");
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options.open(PathBuf::from(lock_path))?;
    fs2::FileExt::try_lock_exclusive(&lock)?; // Do not stall an agent on contention.
    let _lock = StateLock(lock);
    let mut records = load(&path)?;
    let before = serde_json::to_vec(&records).map_err(io::Error::other)?;
    edit(&mut records);
    if before == serde_json::to_vec(&records).map_err(io::Error::other)? {
        return Ok(());
    }
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(&mut temp, &records).map_err(io::Error::other)?;
    temp.flush()?;
    temp.persist(&path).map_err(|e| e.error)?;
    Ok(())
}

#[derive(Debug)]
struct Process {
    pid: u32,
    parent: u32,
    start: String,
    command: String,
}

async fn processes() -> io::Result<Vec<Process>> {
    let mut command = Command::new("ps");
    command
        .args(["-axo", "pid=,ppid=,lstart=,comm="])
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = timeout(Duration::from_millis(700), command.output())
        .await
        .map_err(io::Error::other)??;
    if !output.status.success() || output.stdout.len() > MAX_STORE as usize {
        return Err(io::Error::other("Could not validate agent process."));
    }
    Ok(parse_processes(&String::from_utf8_lossy(&output.stdout)))
}

fn parse_processes(text: &str) -> Vec<Process> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() < 8 {
                return None;
            }
            Some(Process {
                pid: fields[0].parse().ok()?,
                parent: fields[1].parse().ok()?,
                start: fields[2..7].join(" "),
                command: fields[7..].join(" "),
            })
        })
        .collect()
}

fn codex(command: &str) -> bool {
    Path::new(command)
        .file_name()
        .is_some_and(|name| name == "codex")
}

fn root_process(table: &[Process], pane_pid: u32, mut pid: u32) -> Option<&Process> {
    let mut root = None;
    let mut seen = HashSet::new();
    while seen.insert(pid) && seen.len() <= 128 {
        let process = table.iter().find(|p| p.pid == pid)?;
        if codex(&process.command) {
            root = Some(process);
        }
        if pid == pane_pid {
            return root;
        }
        pid = process.parent;
    }
    None
}

async fn pane_identity(tmux: &TmuxClient, pane: &str) -> Option<(String, String, u32)> {
    let text = tmux
        .query(&[
            "display-message",
            "-p",
            "-t",
            pane,
        "#{socket_path}\t#{pid}:#{start_time}\t#{pane_id}\t#{pane_pid}\t#{pane_dead}\t#{pane_current_command}",
        ])
        .await
        .ok()?;
    let fields: Vec<_> = text.trim_end().split('\t').collect();
    if fields.len() != 6
        || fields[0].is_empty()
        || fields[2] != pane
        || fields[4] != "0"
        || !codex(fields[5])
    {
        return None;
    }
    Some((fields[0].into(), fields[1].into(), fields[3].parse().ok()?))
}

impl Engine {
    /// Receive a trusted, explicitly installed callback. `false` means ignored.
    /// Requires inherited tmux context AND Codex ancestry in that pane. No writes
    /// to Codex configuration, no permissions, no transcripts, no agent replies.
    pub async fn record_codex_signal(&self, payload: &str, notification: bool) -> io::Result<bool> {
        let stamp = now(); // Before asynchronous identity queries: reject delayed writes.
        let Some(event) = parse(payload, notification) else {
            return Ok(false);
        };
        let Some(pane) = std::env::var("TMUX_PANE").ok().filter(|s| valid_pane(s)) else {
            return Ok(false);
        };
        let Some(context) = std::env::var("TMUX").ok() else {
            return Ok(false);
        };
        let mut parts = context.rsplitn(3, ',');
        let _session = parts.next();
        let Some(server_pid) = parts.next() else {
            return Ok(false);
        };
        let Some(socket) = parts.next() else {
            return Ok(false);
        };
        let tmux = TmuxClient {
            socket: Some(socket.into()),
            ..self.tmux.clone()
        };
        let Some((socket, server, pane_pid)) = pane_identity(&tmux, &pane).await else {
            return Ok(false);
        };
        if server.split(':').next() != Some(server_pid) {
            return Ok(false);
        }
        let table = processes().await?;
        let Some(root) = root_process(&table, pane_pid, std::process::id()) else {
            return Ok(false);
        };
        let binding = Binding {
            socket,
            server,
            pane,
            pane_pid,
            agent_pid: root.pid,
            agent_start: root.start.clone(),
        };
        let started = std::time::Instant::now();
        loop {
            match update(self.store.state_path(), |records| {
                transition(records, binding.clone(), event.clone(), stamp)
            }) {
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock
                        && started.elapsed() < Duration::from_millis(120) =>
                {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                result => {
                    result?;
                    break;
                }
            }
        }
        Ok(true)
    }

    pub(crate) fn codex_records(&self) -> Vec<Record> {
        load(&path(self.store.state_path()))
            .unwrap_or_default()
            .into_iter()
            .filter(|r| now().saturating_sub(r.stamp) <= MAX_AGE)
            .collect()
    }

    pub(crate) async fn live_codex_record(&self, pane: &str, records: &[Record]) -> Option<Record> {
        if !records.iter().any(|r| r.binding.pane == pane) {
            return None;
        }
        let (socket, server, pid) = pane_identity(&self.tmux, pane).await?;
        let candidate = records
            .iter()
            .filter(|r| {
                r.binding.pane == pane
                    && r.binding.socket == socket
                    && r.binding.server == server
                    && r.binding.pane_pid == pid
            })
            .max_by_key(|r| r.stamp)?;
        let binding = &candidate.binding;
        let table = processes().await.ok()?;
        let root = root_process(&table, pid, binding.agent_pid)?;
        if root.pid != binding.agent_pid || root.start != binding.agent_start {
            return None;
        }
        Some(candidate.clone())
    }

    pub(crate) fn reconcile_codex_record(&self, record: &Record) {
        // Do not overwrite a callback that arrived while the viewport was captured.
        let _ = update(self.store.state_path(), |records| {
            if let Some(current) = records.iter_mut().find(|r| {
                r.binding == record.binding
                    && r.session == record.session
                    && r.turn == record.turn
                    && r.stamp == record.stamp
            }) {
                current.pending.clone_from(&record.pending);
            }
        });
    }
}

fn valid_pane(pane: &str) -> bool {
    pane.strip_prefix('%')
        .is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
}

impl Record {
    /// Screen uncertainty never downgrades valid lifecycle evidence. Pending hooks
    /// need live UI confirmation; missing UI alone is NOT permission resolution.
    pub(crate) fn apply(&mut self, state: &mut WorkItemState, screen: Option<&str>) {
        let terminal_running = state.status == AgentStatus::Running;
        let terminal_waiting = state.status == AgentStatus::WaitingForInput;
        if self.phase == Phase::Idle && terminal_running {
            return; // A missing submit callback is not affirmative idle evidence.
        }
        if let Some(pending) = &mut self.pending {
            if terminal_waiting
                || screen.is_some_and(|screen| question_dialog(screen, &pending.questions))
            {
                pending.confirmed = true;
            } else if terminal_running && pending.confirmed {
                // Includes denied approvals: Codex has no denial-resolved hook.
                self.pending = None;
            }
        }
        if terminal_waiting {
            return; // Preserve exact visible approval text for existing y/n safety.
        }
        state.completion_fingerprint = None;
        state.attention_prompt = None;
        if let Some(pending) = &self.pending
            && pending.confirmed
        {
            state.status = AgentStatus::WaitingForInput;
            state.status_detail = "Codex input request remains pending.".into();
            if !pending.questions.is_empty() {
                state.attention_prompt = Some(
                    pending
                        .questions
                        .iter()
                        .map(|q| {
                            let options = q
                                .options
                                .iter()
                                .enumerate()
                                .map(|(i, (label, description))| {
                                    format!("{}. {label} — {description}", i + 1)
                                })
                                .collect::<Vec<_>>()
                                .join("\n");
                            format!("{}\n\nOptions:\n{options}", q.text)
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n"),
                );
            }
            return;
        }
        let (status, detail) = match self.phase {
            Phase::Idle => (
                AgentStatus::Idle,
                "Codex session started; no submitted turn observed.",
            ),
            Phase::Running => (
                AgentStatus::Running,
                "Codex lifecycle signals report an active turn.",
            ),
            Phase::Interrupted => (AgentStatus::Idle, "Codex turn was interrupted."),
            Phase::Complete => (AgentStatus::Complete, "Codex confirmed turn completion."),
            Phase::Ended => (AgentStatus::Idle, "Codex session ended."),
        };
        state.status = status;
        state.status_detail = detail.into();
        if self.phase == Phase::Complete {
            let mut hasher = DefaultHasher::new();
            self.binding.hash_identity(&mut hasher);
            self.session.hash(&mut hasher);
            self.turn.hash(&mut hasher);
            state.completion_fingerprint = Some(hasher.finish());
        }
    }
}

impl Binding {
    fn hash_identity(&self, hasher: &mut impl Hasher) {
        self.socket.hash(hasher);
        self.server.hash(hasher);
        self.agent_pid.hash(hasher);
        self.agent_start.hash(hasher);
    }
}

fn question_dialog(screen: &str, questions: &[Question]) -> bool {
    if questions.is_empty() {
        return false;
    }
    let normalized = screen.split_whitespace().collect::<Vec<_>>().join(" ");
    // A pending native tool payload plus matching live question/options AND
    // interactive dialog chrome; quoted prose/history is not sufficient.
    let footer = "tab to add notes | enter to submit answer | esc to interrupt";
    normalized.ends_with(footer)
        && screen.lines().any(|line| {
            line.trim().starts_with("Question ") && line.trim().ends_with(" unanswered)")
        })
        && questions.iter().any(|question| {
            normalized.contains(
                &question
                    .text
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
            ) && question.options.iter().all(|(label, _)| {
                normalized.contains(&label.split_whitespace().collect::<Vec<_>>().join(" "))
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AttentionTracker, PaneAvailability, WorkItem, WorkItemKind};

    fn binding() -> Binding {
        Binding {
            socket: "/tmp/private-tmux".into(),
            server: "100:200".into(),
            pane: "%1".into(),
            pane_pid: 101,
            agent_pid: 102,
            agent_start: "start".into(),
        }
    }
    fn event(name: &str, turn: &str) -> Event {
        Event {
            session: "session".into(),
            turn: (!turn.is_empty()).then(|| turn.into()),
            name: name.into(),
            call: Some("call".into()),
            questions: Vec::new(),
        }
    }
    fn active() -> Record {
        let mut records = Vec::new();
        transition(&mut records, binding(), event("SessionStart", ""), 1);
        transition(
            &mut records,
            binding(),
            event("UserPromptSubmit", "turn1"),
            2,
        );
        records.pop().unwrap()
    }
    fn state(status: AgentStatus) -> WorkItemState {
        WorkItemState {
            item: WorkItem {
                id: "work".into(),
                title: "Description".into(),
                repository: "/repo".into(),
                workspace: "/repo".into(),
                branch: None,
                kind: WorkItemKind::Implementation,
                pane_id: "%1".into(),
            },
            status,
            pane: PaneAvailability::Present,
            status_detail: String::new(),
            attention_prompt: None,
            completion_fingerprint: None,
        }
    }
    fn question_event() -> Event {
        parse(
            r#"{"session_id":"session","turn_id":"turn1","hook_event_name":"PreToolUse",
            "tool_name":"request_user_input","tool_use_id":"call","permission_mode":"plan",
            "tool_input":{"questions":[{"id":"tests","question":"Which strategy?","options":[
            {"label":"Unit tests","description":"Fast tests"},
            {"label":"Integration","description":"Full flow"}]}]}}"#,
            false,
        )
        .unwrap()
    }
    #[test]
    fn stop_continuation_is_not_completion_and_quiet_turns_remain_running() {
        let mut records = vec![active()];
        for stamp in [3, 15_000_000] {
            transition(&mut records, binding(), event("Stop", "turn1"), stamp);
            assert_eq!(records[0].phase, Phase::Running);
        }
        transition(
            &mut records,
            binding(),
            event("Complete", "turn1"),
            16_000_000,
        );
        assert_eq!(records[0].phase, Phase::Complete);
        transition(
            &mut records,
            binding(),
            event("PostToolUse", "turn1"),
            17_000_000,
        );
        assert_eq!(records[0].phase, Phase::Complete);
    }
    #[test]
    fn scroll_copy_mode_and_multiline_drafts_do_not_replace_lifecycle_status() {
        for screen in [
            None,
            Some("old history"),
            Some("› draft\nsecond line"),
            Some("Worked for 2s"),
        ] {
            let mut record = active();
            let mut observed = state(AgentStatus::Unknown);
            record.apply(&mut observed, screen);
            assert_eq!(observed.status, AgentStatus::Running);
            // A stale visible completion must not finish an active native turn.
            observed.status = AgentStatus::Complete;
            record.apply(&mut observed, screen);
            assert_eq!(observed.status, AgentStatus::Running);
        }
    }
    #[test]
    fn completion_identity_and_acknowledgement_survive_viewport_changes() {
        let mut records = vec![active()];
        transition(&mut records, binding(), event("Complete", "turn1"), 3);
        let mut observed = state(AgentStatus::Unknown);
        records[0].apply(&mut observed, None);
        let fingerprint = observed.completion_fingerprint.unwrap();
        let mut tracker = AttentionTracker::default();
        tracker.observe(&[observed.clone()]);
        let ack = tracker.capture(&observed).unwrap();
        tracker.acknowledge(&ack).unwrap();
        for screen in ["old history", "draft\nsecond line", "resized UI"] {
            observed.status = AgentStatus::Unknown;
            records[0].apply(&mut observed, Some(screen));
            assert_eq!(observed.completion_fingerprint, Some(fingerprint));
            assert!(tracker.observe(&[observed.clone()]).is_empty());
        }
        transition(
            &mut records,
            binding(),
            event("UserPromptSubmit", "turn2"),
            4,
        );
        records[0].apply(&mut observed, None);
        tracker.observe(&[observed.clone()]);
        transition(&mut records, binding(), event("Complete", "turn2"), 5);
        records[0].apply(&mut observed, None);
        assert_ne!(observed.completion_fingerprint, Some(fingerprint));
        assert_eq!(tracker.observe(&[observed]).len(), 1);
    }
    #[test]
    fn late_events_cannot_resurrect_old_turns_or_overwrite_newer_callbacks() {
        let mut records = vec![active()];
        transition(
            &mut records,
            binding(),
            event("UserPromptSubmit", "turn2"),
            10,
        );
        for name in ["Complete", "Interrupt", "PostToolUse", "UserPromptSubmit"] {
            transition(&mut records, binding(), event(name, "turn1"), 11);
            assert_eq!(records[0].turn.as_deref(), Some("turn2"));
            assert_eq!(records[0].phase, Phase::Running);
        }
        transition(&mut records, binding(), event("Complete", "turn2"), 9);
        assert_eq!(records[0].phase, Phase::Running);
    }
    #[test]
    fn session_changes_and_process_restarts_do_not_adopt_delayed_completions() {
        let mut records = vec![active()];
        let mut start = event("SessionStart", "");
        start.session = "new-session".into();
        transition(&mut records, binding(), start, 3);
        transition(&mut records, binding(), event("SessionStart", ""), 4);
        transition(&mut records, binding(), event("Complete", "turn1"), 5);
        assert_eq!(records[0].session, "new-session");
        assert_eq!(records[0].phase, Phase::Idle);
        let mut restarted = binding();
        restarted.agent_start = "new start with reused PID".into();
        transition(
            &mut records,
            restarted.clone(),
            event("Complete", "turn1"),
            6,
        );
        assert_eq!(records.len(), 1);
        transition(&mut records, restarted, event("SessionStart", ""), 7);
        assert_eq!(records.len(), 2);
    }
    #[test]
    fn interruptions_and_session_end_clear_pending_without_implying_success() {
        for name in ["Interrupt", "SessionEnd"] {
            let mut records = vec![active()];
            transition(&mut records, binding(), question_event(), 3);
            transition(&mut records, binding(), event(name, "turn1"), 4);
            transition(&mut records, binding(), event("Complete", "turn1"), 5);
            transition(&mut records, binding(), event("PostToolUse", "turn1"), 6);
            let mut observed = state(AgentStatus::Unknown);
            records[0].apply(&mut observed, None);
            assert_eq!(observed.status, AgentStatus::Idle);
            assert!(records[0].pending.is_none());
            assert!(observed.completion_fingerprint.is_none());
        }
    }
    #[test]
    fn permission_hooks_need_confirmation_and_auto_approval_never_creates_attention() {
        let mut records = vec![active()];
        transition(
            &mut records,
            binding(),
            event("PermissionRequest", "turn1"),
            3,
        );
        let mut observed = state(AgentStatus::Unknown);
        records[0].apply(&mut observed, None);
        assert_eq!(observed.status, AgentStatus::Running);
        transition(&mut records, binding(), event("PostToolUse", "turn1"), 4);
        assert!(!records[0].pending.as_ref().unwrap().confirmed);
    }
    #[test]
    fn confirmed_approval_survives_hidden_ui_but_live_resumed_activity_resolves_denial() {
        let mut records = vec![active()];
        transition(
            &mut records,
            binding(),
            event("PermissionRequest", "turn1"),
            3,
        );
        let mut observed = state(AgentStatus::WaitingForInput);
        observed.attention_prompt = Some("Exact captured approval".into());
        records[0].apply(&mut observed, None);
        assert_eq!(
            observed.attention_prompt.as_deref(),
            Some("Exact captured approval")
        );
        observed.status = AgentStatus::Unknown;
        records[0].apply(&mut observed, None);
        assert_eq!(observed.status, AgentStatus::WaitingForInput);
        observed.status = AgentStatus::Running;
        records[0].apply(&mut observed, None);
        assert_eq!(observed.status, AgentStatus::Running);
        assert!(records[0].pending.is_none());
    }
    #[test]
    fn structured_options_require_current_dialog_and_resolve_only_matching_tool() {
        let mut records = vec![active()];
        transition(&mut records, binding(), question_event(), 3);
        let mut observed = state(AgentStatus::Unknown);
        records[0].apply(
            &mut observed,
            Some("Which strategy? Unit tests Integration\n? for shortcuts"),
        );
        assert_eq!(observed.status, AgentStatus::Running);
        records[0].apply(
            &mut observed,
            Some("Question 1/1 (1 unanswered)\nWhich strategy?\n› 1. Unit tests\n2. Integration\ntab to add notes | enter to submit answer | esc to interrupt"),
        );
        assert_eq!(observed.status, AgentStatus::WaitingForInput);
        assert!(
            observed
                .attention_prompt
                .unwrap()
                .contains("Integration — Full flow")
        );
        let mut unrelated = event("PostToolUse", "turn1");
        unrelated.call = Some("other".into());
        transition(&mut records, binding(), unrelated, 4);
        assert!(records[0].pending.is_some());
        transition(&mut records, binding(), event("PostToolUse", "turn1"), 5);
        assert!(records[0].pending.is_none());
    }
    #[test]
    fn multiple_questions_retain_all_options_and_never_store_answers() {
        let mut question = question_event();
        question.questions.push(Question {
            text: "Which files?".into(),
            options: vec![
                ("Core".into(), "Engine changes".into()),
                ("TUI".into(), "Display changes".into()),
            ],
        });
        let mut records = vec![active()];
        transition(&mut records, binding(), question, 3);
        let mut observed = state(AgentStatus::Unknown);
        records[0].apply(&mut observed, Some("Question 1/2 (2 unanswered)\nWhich strategy?\n› 1. Unit tests\n2. Integration\ntab to add notes | enter to submit answer | esc to interrupt"));
        let prompt = observed.attention_prompt.unwrap();
        assert!(prompt.contains("Which files?"));
        assert!(prompt.contains("Core — Engine changes"));
        let answered = parse(r#"{"session_id":"session","turn_id":"turn1","hook_event_name":"PostToolUse",
            "tool_use_id":"call","tool_name":"request_user_input","tool_response":"secret custom answer"}"#, false).unwrap();
        transition(&mut records, binding(), answered, 4);
        assert!(records[0].pending.is_none());
        assert!(!serde_json::to_string(&records).unwrap().contains("secret"));
    }
    #[test]
    fn unrelated_post_tool_cannot_clear_a_permission_candidate_without_a_call_id() {
        let mut records = vec![active()];
        transition(
            &mut records,
            binding(),
            event("PermissionRequest", "turn1"),
            3,
        );
        let mut observed = state(AgentStatus::WaitingForInput);
        records[0].apply(&mut observed, None);
        transition(&mut records, binding(), event("PostToolUse", "turn1"), 4);
        assert!(records[0].pending.as_ref().unwrap().confirmed);
        observed.status = AgentStatus::Unknown;
        records[0].apply(&mut observed, None);
        assert_eq!(observed.status, AgentStatus::WaitingForInput);
    }
    #[test]
    fn unsupported_subagents_and_malformed_payloads_are_ignored() {
        for payload in [
            "garbage",
            "{}",
            r#"{"session_id":"s","hook_event_name":"SubagentStop"}"#,
            r#"{"session_id":"s","hook_event_name":"SessionStart","agent_id":"child"}"#,
            r#"{"session_id":"s","hook_event_name":"Interrupt"}"#,
        ] {
            assert!(parse(payload, false).is_none(), "{payload}");
        }
        assert!(parse(&"x".repeat(MAX_PAYLOAD + 1), false).is_none());
        assert!(parse(r#"{"type":"other","thread-id":"s","turn-id":"t"}"#, true).is_none());
        let complete = parse(
            r#"{"type":"agent-turn-complete","thread-id":"s","turn-id":"t",
            "input-messages":["secret"],"last-assistant-message":"secret"}"#,
            true,
        )
        .unwrap();
        assert_eq!(complete.name, "Complete");
        assert!(complete.questions.is_empty());
    }
    #[test]
    fn permission_mode_is_not_collaboration_mode_and_invalid_options_are_ignored() {
        let valid = serde_json::json!({"session_id":"s","turn_id":"t","hook_event_name":"PreToolUse",
            "tool_name":"request_user_input","permission_mode":"default","tool_input":{"questions":[
            {"id":"q","question":"Which?","options":[{"label":"A","description":"First"}]}]}});
        assert_eq!(parse(&valid.to_string(), false).unwrap().questions.len(), 1);
        let mut invalid = valid;
        invalid["permission_mode"] = "plan".into();
        invalid["tool_input"]["questions"][0]["options"] = serde_json::json!([]);
        assert!(
            parse(&invalid.to_string(), false)
                .unwrap()
                .questions
                .is_empty()
        );
        invalid["tool_name"] = "request_user_input_async".into();
        assert!(
            parse(&invalid.to_string(), false)
                .unwrap()
                .questions
                .is_empty()
        );
    }
    #[test]
    fn binding_requires_exact_process_ancestry_not_workspace_or_inherited_pane_alone() {
        let table = parse_processes(
            "101 1 Sat Oct 3 10:00:00 2026 zsh\n102 101 Sat Oct 3 10:00:01 2026 /bin/codex\n103 102 Sat Oct 3 10:00:02 2026 sh\n104 103 Sat Oct 3 10:00:03 2026 /bin/workbench\n105 1 Sat Oct 3 10:00:04 2026 /bin/codex",
        );
        assert_eq!(root_process(&table, 101, 104).unwrap().pid, 102);
        assert!(root_process(&table, 101, 105).is_none());
        assert!(root_process(&table, 999, 104).is_none());
        assert!(root_process(&table, 101, 101).is_none());
    }
    #[test]
    fn storage_is_bounded_private_and_recovers_across_engine_instances() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("items.json");
        update(&state, |records| records.push(active())).unwrap();
        assert_eq!(load(&path(&state)).unwrap()[0].phase, Phase::Running);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path(&state)).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        update(&state, |records| {
            transition(records, binding(), event("Complete", "turn1"), 3)
        })
        .unwrap();
        assert_eq!(load(&path(&state)).unwrap()[0].phase, Phase::Complete);
        fs::write(path(&state), "broken").unwrap();
        assert!(Engine::new(&state).codex_records().is_empty());
        assert!(update(&state, |_| {}).is_err()); // Never silently erase corruption.
    }
    #[test]
    fn expiry_and_record_limits_prevent_permanent_or_unbounded_state() {
        let mut records = vec![active()];
        transition(
            &mut records,
            binding(),
            event("Complete", "turn1"),
            MAX_AGE + 3,
        );
        assert!(records.is_empty());
        for n in 0..MAX_RECORDS + 10 {
            let mut b = binding();
            b.agent_pid = n as u32;
            transition(&mut records, b, event("SessionStart", ""), n as u64 + 1);
        }
        assert_eq!(records.len(), MAX_RECORDS);
    }
}
