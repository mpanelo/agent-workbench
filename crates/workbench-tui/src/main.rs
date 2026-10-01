use std::{io, sync::Arc, time::Duration};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tokio::{
    sync::watch,
    task::JoinSet,
    time::{self, MissedTickBehavior},
};
use workbench_core::{Engine, Snapshot, WorkItemState, attention_items, validate_agent_input};

mod cli;
mod interaction;
mod review;
mod ui;

type DiscoveryState = Option<Result<Snapshot, String>>;

#[derive(Clone, Debug, Default)]
struct AppState {
    discovery: DiscoveryState,
    work_items: Option<Result<Vec<WorkItemState>, String>>,
    attention: Vec<WorkItemState>,
}

impl AppState {
    fn from_refresh(
        discovery: Result<Snapshot, String>,
        work_items: Result<Vec<WorkItemState>, String>,
    ) -> Self {
        let attention = work_items
            .as_ref()
            .map(|items| attention_items(items))
            .unwrap_or_default();
        Self {
            discovery: Some(discovery),
            work_items: Some(work_items),
            attention,
        }
    }

    fn items(&self) -> &[WorkItemState] {
        self.work_items
            .as_ref()
            .and_then(|items| items.as_ref().ok())
            .map_or(&[], Vec::as_slice)
    }

    fn items_for(&self, view: ui::View) -> &[WorkItemState] {
        match view {
            ui::View::Attention => &self.attention,
            _ => self.items(),
        }
    }
}

enum RunExit {
    Quit,
    Focus(String),
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    match start().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn start() -> io::Result<()> {
    let options = cli::parse(std::env::args_os().skip(1)).map_err(io::Error::other)?;
    if options.command == cli::Command::Help {
        print!("{}", cli::HELP);
        return Ok(());
    }
    let engine = Arc::new(Engine::new(options.state_path().map_err(io::Error::other)?));
    match options.command {
        cli::Command::Register(item) => {
            let id = item.id.clone();
            engine.register_work_item(item).map_err(io::Error::other)?;
            println!("Registered {id}.");
            return Ok(());
        }
        cli::Command::List => {
            // Listing remains useful offline, with unavailable/UNKNOWN observations.
            let snapshot = engine.discover().await.ok();
            let items = engine
                .observe_work_item_states(snapshot.as_ref())
                .await
                .map_err(io::Error::other)?;
            if items.is_empty() {
                println!("No work items registered.");
            }
            for state in items {
                let item = state.item;
                println!(
                    "{}\t{}\t{}\t{} ({})\n  {}\n  Status: {}\n  Repository: {}\n  Workspace: {}\n  Branch: {}",
                    item.id,
                    state.status,
                    item.kind,
                    item.pane_id,
                    state.pane,
                    item.title,
                    state.status_detail,
                    ui::display_path(&item.repository),
                    ui::display_path(&item.workspace),
                    item.branch.as_deref().unwrap_or("unavailable")
                );
            }
            return Ok(());
        }
        cli::Command::Run => {}
        cli::Command::Help => unreachable!("help was handled above"),
    }
    let mut interaction = interaction::Interaction::default();
    let mut view = ui::View::default();
    let mut reviews = review::ReviewUi::new(options.diff_base);
    loop {
        let mut terminal = init_terminal()?;
        let result = run(
            &mut terminal,
            Arc::clone(&engine),
            &mut interaction,
            &mut view,
            &mut reviews,
        )
        .await;
        restore_terminal()?;
        match result? {
            RunExit::Quit => return Ok(()),
            RunExit::Focus(id) => {
                interaction.message = Some(match engine.focus_work_item(&id).await {
                    Ok(()) => format!(
                        "Opened {id}. Use tmux navigation to return, or detach when attached from outside."
                    ),
                    Err(error) => format!("Could not open {id}: {error}"),
                });
                interaction.reveal_selection = true;
            }
        }
    }
}

fn init_terminal() -> io::Result<ratatui::DefaultTerminal> {
    let terminal = match ratatui::try_init() {
        Ok(terminal) => terminal,
        Err(error) => {
            ratatui::restore();
            return Err(error);
        }
    };
    if let Err(error) = crossterm::execute!(io::stdout(), event::EnableBracketedPaste) {
        ratatui::restore();
        return Err(error);
    }
    Ok(terminal)
}

fn restore_terminal() -> io::Result<()> {
    let paste = crossterm::execute!(io::stdout(), event::DisableBracketedPaste);
    let restore = ratatui::try_restore();
    paste.and(restore)
}

async fn run(
    terminal: &mut ratatui::DefaultTerminal,
    engine: Arc<Engine>,
    interaction: &mut interaction::Interaction,
    view: &mut ui::View,
    reviews: &mut review::ReviewUi,
) -> io::Result<RunExit> {
    let (sender, mut receiver) = watch::channel::<AppState>(AppState::default());
    let discovery_engine = Arc::clone(&engine);
    let discovery = tokio::spawn(async move {
        let mut refresh = time::interval(Duration::from_secs(2));
        refresh.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            refresh.tick().await;
            let discovery = discovery_engine
                .discover()
                .await
                .map_err(|error| error.to_string());
            let work_items = discovery_engine
                .observe_work_item_states(discovery.as_ref().ok())
                .await
                .map_err(|error| error.to_string());
            if sender
                .send(AppState::from_refresh(discovery, work_items))
                .is_err()
            {
                break;
            }
        }
    });
    let result = event_loop(terminal, &mut receiver, engine, interaction, view, reviews).await;
    discovery.abort();
    // Wait for cancellation so an in-flight command is dropped and killed.
    let _ = discovery.await;
    result
}

async fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    receiver: &mut watch::Receiver<AppState>,
    engine: Arc<Engine>,
    interaction: &mut interaction::Interaction,
    view: &mut ui::View,
    reviews: &mut review::ReviewUi,
) -> io::Result<RunExit> {
    let mut state = AppState::default();
    let mut scroll = 0_u16;
    let mut redraw = true;
    let mut sends = JoinSet::new();
    let mut diffs = JoinSet::new();
    let mut input_tick = time::interval(Duration::from_millis(100));
    input_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        if redraw {
            terminal.draw(|frame| {
                if reviews.is_open() {
                    reviews.render(frame);
                } else {
                    match *view {
                        ui::View::Work => ui::render_work(frame, &state, &mut scroll, interaction),
                        ui::View::Attention => {
                            ui::render_attention(frame, &state, &mut scroll, interaction)
                        }
                        ui::View::Sessions => ui::render(frame, &state.discovery, &mut scroll),
                    }
                }
            })?;
            redraw = false;
        }
        tokio::select! {
            completed = diffs.join_next(), if !diffs.is_empty() => {
                match completed {
                    Some(Ok((ticket, result))) => reviews.finish(ticket, result),
                    Some(Err(error)) if !error.is_cancelled() => reviews.fail_loading("Git capture task stopped unexpectedly; reload the diff.".into()),
                    _ => {},
                }
                redraw = true;
            }
            changed = receiver.changed() => {
                changed.map_err(|_| io::Error::other("Discovery task stopped unexpectedly"))?;
                let previous_position = state.items_for(*view).iter().position(|item| Some(&item.item.id) == interaction.selected_id.as_ref());
                let updated = receiver.borrow_and_update().clone();
                let attention_changed = state.attention != updated.attention;
                state = updated;
                interaction.sync(state.items_for(*view));
                let position = state.items_for(*view).iter().position(|item| Some(&item.item.id) == interaction.selected_id.as_ref());
                interaction.reveal_selection |= position != previous_position || (*view == ui::View::Attention && attention_changed);
                redraw = true;
            }
            completed = sends.join_next(), if !sends.is_empty() => {
                interaction.sending = false;
                match completed {
                    Some(Ok((id, result))) => match result {
                        Ok(()) => { interaction.message = Some(format!("Reply submitted to {id}.")); interaction.draft = None; }
                        Err(error) => interaction.message = Some(format!("Reply to {id} failed: {error}. Not retried; check the pane before resending.")),
                    },
                    _ => interaction.message = Some("Input task stopped unexpectedly. Check the pane before resending.".into()),
                }
                redraw = true;
            }
            _ = input_tick.tick() => {
                while event::poll(Duration::ZERO)? {
                    redraw = true;
                    let event = event::read()?;
                    if matches!(event, Event::Resize(_, _)) { interaction.reveal_selection = true; }
                    if let Event::Paste(text) = &event
                        && !interaction.sending
                        && let Some(draft) = &mut interaction.draft
                        && let Err(error) = draft.append(text)
                    {
                        interaction.message = Some(error);
                    }
                    if let Event::Key(key) = event {
                        if key.kind == KeyEventKind::Release {
                            continue;
                        }
                        if reviews.is_open() {
                            if key.code == KeyCode::Char('q') || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)) {
                                return Ok(RunExit::Quit);
                            }
                            if reviews.key(key, terminal.size()?.height) == review::ReviewIntent::Reload {
                                diffs.abort_all();
                                let (ticket, id) = reviews.reload().expect("review is open");
                                let engine = Arc::clone(&engine);
                                let base = reviews.base.clone();
                                diffs.spawn(async move { (ticket, engine.diff(&id, base.as_deref()).await.map_err(|error| error.to_string())) });
                            }
                            if !reviews.is_open() { diffs.abort_all(); }
                            continue;
                        }
                        if let Some(draft) = &mut interaction.draft {
                            match key.code {
                                KeyCode::Esc | KeyCode::Char('c') if key.code == KeyCode::Esc || key.modifiers.contains(KeyModifiers::CONTROL) => {
                                    if !interaction.sending { interaction.draft = None; interaction.message = None; }
                                }
                                KeyCode::Enter if !interaction.sending => {
                                    match validate_agent_input(&draft.text) {
                                        Err(error) => interaction.message = Some(error.to_string()),
                                        Ok(()) => {
                                            let id = draft.item_id.clone();
                                            let text = draft.text.clone();
                                            let engine = Arc::clone(&engine);
                                            interaction.sending = true;
                                            interaction.message = Some(format!("Sending reply to {id}…"));
                                            sends.spawn(async move { let result = engine.send_agent_input(&id, &text).await; (id, result) });
                                        }
                                    }
                                }
                                _ if !interaction.sending => if let Err(error) = draft.edit(key) { interaction.message = Some(error); },
                                _ => {}
                            }
                            continue;
                        }
                        match key.code {
                            KeyCode::Char('q') | KeyCode::Esc => return Ok(RunExit::Quit),
                            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(RunExit::Quit),
                            _ => {
                                if let Some(id) = navigate(key, &state, view, &mut scroll, interaction, terminal.size()?.height) { return Ok(RunExit::Focus(id)); }
                                if let Some(id) = interaction.review_requested.take()
                                    && let Some(ticket) = reviews.open(id.clone()) {
                                        diffs.abort_all();
                                        let engine = Arc::clone(&engine);
                                        let base = reviews.base.clone();
                                        diffs.spawn(async move { (ticket, engine.diff(&id, base.as_deref()).await.map_err(|error| error.to_string())) });
                                }
                            },
                        }
                    }
                }
            }
        }
    }
}

/// Route list actions against the visible subset, never the hidden WORK list.
fn navigate(
    key: impl Into<KeyEvent>,
    state: &AppState,
    view: &mut ui::View,
    scroll: &mut u16,
    interaction: &mut interaction::Interaction,
    height: u16,
) -> Option<String> {
    let key = key.into();
    if interaction.draft.is_some() {
        return None;
    }
    if key.modifiers == KeyModifiers::CONTROL {
        let half_page = (height.saturating_sub(2) / 2).max(1);
        match key.code {
            KeyCode::Char('d') => *scroll = scroll.saturating_add(half_page),
            KeyCode::Char('u') => *scroll = scroll.saturating_sub(half_page),
            _ => return None,
        }
        interaction.reveal_selection = false;
        return None;
    }
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
    {
        return None;
    }
    let items = state.items_for(*view);
    match key.code {
        KeyCode::Down | KeyCode::Char('j') if view.is_item_view() => {
            interaction.move_selection(items, 1)
        }
        KeyCode::Up | KeyCode::Char('k') if view.is_item_view() => {
            interaction.move_selection(items, -1)
        }
        KeyCode::Enter if view.is_item_view() => {
            if let Some(selected) = interaction.selected(items) {
                return Some(selected.item.id.clone());
            }
            interaction.message = Some("Select an item in the current view first.".into());
        }
        KeyCode::Char('r') if view.is_item_view() => interaction.begin_reply(items),
        KeyCode::Tab if view.is_item_view() => interaction.next_attention(items),
        KeyCode::Char('d') if view.is_item_view() => {
            interaction.review_requested = interaction
                .selected(items)
                .map(|selected| selected.item.id.clone());
            interaction.message = if interaction.review_requested.is_none() {
                Some("Select an item in the current view first.".into())
            } else {
                None
            };
        }
        KeyCode::Down => *scroll = scroll.saturating_add(1),
        KeyCode::Up => *scroll = scroll.saturating_sub(1),
        KeyCode::PageDown => *scroll = scroll.saturating_add(height.saturating_sub(3)),
        KeyCode::PageUp => *scroll = scroll.saturating_sub(height.saturating_sub(3)),
        KeyCode::Home => {
            *scroll = 0;
            if view.is_item_view() {
                interaction.selected_id = None;
                interaction.sync(items);
            }
        }
        KeyCode::Char('a' | 'w' | 's') => {
            *view = match key.code {
                KeyCode::Char('a') => ui::View::Attention,
                KeyCode::Char('w') => ui::View::Work,
                _ => ui::View::Sessions,
            };
            *scroll = 0;
            interaction.sync(state.items_for(*view));
            interaction.reveal_selection = true;
        }
        _ => {}
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use workbench_core::{AgentStatus, PaneAvailability, WorkItem, WorkItemKind};

    fn state(items: &[(&str, AgentStatus)]) -> AppState {
        AppState::from_refresh(
            Ok(Snapshot::default()),
            Ok(items
                .iter()
                .map(|(id, status)| WorkItemState {
                    item: WorkItem {
                        id: (*id).into(),
                        title: "Task".into(),
                        repository: "/work".into(),
                        workspace: "/work".into(),
                        branch: None,
                        kind: WorkItemKind::Implementation,
                        pane_id: "%14".into(),
                    },
                    status: *status,
                    pane: PaneAvailability::Present,
                    status_detail: "Observed locally.".into(),
                    attention_prompt: None,
                })
                .collect()),
        )
    }

    #[test]
    fn vim_half_page_scroll_works_in_every_view_without_changing_selection() {
        let state = state(&[("A", AgentStatus::WaitingForInput)]);
        for mut view in [ui::View::Attention, ui::View::Work, ui::View::Sessions] {
            let original_view = view;
            let mut interaction = interaction::Interaction::default();
            interaction.sync(state.items_for(view));
            let selected = interaction.selected_id.clone();
            let mut scroll = 0;
            for (key, expected) in [('d', 11), ('d', 22), ('u', 11), ('u', 0), ('u', 0)] {
                assert!(
                    navigate(
                        KeyEvent::new(KeyCode::Char(key), KeyModifiers::CONTROL),
                        &state,
                        &mut view,
                        &mut scroll,
                        &mut interaction,
                        24,
                    )
                    .is_none()
                );
                assert_eq!(scroll, expected);
                assert_eq!(view, original_view);
                assert_eq!(interaction.selected_id, selected);
                assert!(interaction.message.is_none());
                assert!(interaction.draft.is_none());
                assert!(!interaction.reveal_selection);
            }
            navigate(
                KeyCode::PageDown,
                &state,
                &mut view,
                &mut scroll,
                &mut interaction,
                24,
            );
            assert_eq!(scroll, 21);
            navigate(
                KeyCode::PageUp,
                &state,
                &mut view,
                &mut scroll,
                &mut interaction,
                24,
            );
            assert_eq!(scroll, 0);
        }
    }

    #[test]
    fn vim_scroll_saturates_and_moves_at_least_one_row_on_tiny_terminals() {
        let state = state(&[]);
        let mut view = ui::View::Attention;
        let mut interaction = interaction::Interaction::default();
        for height in [0, 1, 2, 3, 4] {
            let mut scroll = 0;
            navigate(
                KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
                &state,
                &mut view,
                &mut scroll,
                &mut interaction,
                height,
            );
            assert_eq!(scroll, 1);
            scroll = u16::MAX;
            navigate(
                KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
                &state,
                &mut view,
                &mut scroll,
                &mut interaction,
                height,
            );
            assert_eq!(scroll, u16::MAX);
        }
    }

    #[test]
    fn modified_keys_and_reply_drafts_do_not_trigger_list_actions() {
        let state = state(&[("A", AgentStatus::WaitingForInput)]);
        let mut view = ui::View::Attention;
        let mut interaction = interaction::Interaction::default();
        interaction.sync(state.items_for(view));
        let mut scroll = 10;
        for key in [
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::ALT),
            KeyEvent::new(
                KeyCode::Char('d'),
                KeyModifiers::CONTROL | KeyModifiers::ALT,
            ),
        ] {
            navigate(key, &state, &mut view, &mut scroll, &mut interaction, 24);
            assert_eq!(scroll, 10);
            assert!(interaction.message.is_none());
            assert!(interaction.draft.is_none());
        }
        interaction.begin_reply(state.items_for(view));
        interaction.draft.as_mut().unwrap().append("yes").unwrap();
        for ch in ['d', 'u'] {
            navigate(
                KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL),
                &state,
                &mut view,
                &mut scroll,
                &mut interaction,
                24,
            );
            assert_eq!(scroll, 10);
            assert_eq!(interaction.draft.as_ref().unwrap().text, "yes");
        }
    }

    #[test]
    fn attention_is_default_and_all_actions_target_the_visible_queue() {
        let state = state(&[
            ("hidden", AgentStatus::Running),
            ("B", AgentStatus::Complete),
            ("C", AgentStatus::WaitingForInput),
        ]);
        let mut view = ui::View::default();
        assert_eq!(view, ui::View::Attention);
        let mut interaction = interaction::Interaction::default();
        interaction.sync(state.items_for(view));
        let mut scroll = 0;
        assert_eq!(interaction.selected_id.as_deref(), Some("B"));
        navigate(
            KeyCode::Char('j'),
            &state,
            &mut view,
            &mut scroll,
            &mut interaction,
            24,
        );
        assert_eq!(
            navigate(
                KeyCode::Enter,
                &state,
                &mut view,
                &mut scroll,
                &mut interaction,
                24
            )
            .as_deref(),
            Some("C")
        );
        navigate(
            KeyCode::Char('r'),
            &state,
            &mut view,
            &mut scroll,
            &mut interaction,
            24,
        );
        assert_eq!(interaction.draft.as_ref().unwrap().item_id, "C");
        interaction.draft = None;
        navigate(
            KeyCode::Tab,
            &state,
            &mut view,
            &mut scroll,
            &mut interaction,
            24,
        );
        assert_eq!(interaction.selected_id.as_deref(), Some("B"));
        navigate(
            KeyCode::Char('d'),
            &state,
            &mut view,
            &mut scroll,
            &mut interaction,
            24,
        );
        assert_eq!(interaction.review_requested.as_deref(), Some("B"));
        navigate(
            KeyCode::Char('w'),
            &state,
            &mut view,
            &mut scroll,
            &mut interaction,
            24,
        );
        assert_eq!(view, ui::View::Work);
        navigate(
            KeyCode::Home,
            &state,
            &mut view,
            &mut scroll,
            &mut interaction,
            24,
        );
        assert_eq!(interaction.selected_id.as_deref(), Some("hidden"));
        navigate(
            KeyCode::Char('a'),
            &state,
            &mut view,
            &mut scroll,
            &mut interaction,
            24,
        );
        assert_eq!(interaction.selected_id.as_deref(), Some("B"));
        navigate(
            KeyCode::Char('s'),
            &state,
            &mut view,
            &mut scroll,
            &mut interaction,
            24,
        );
        assert_eq!(view, ui::View::Sessions);
        assert!(
            navigate(
                KeyCode::Enter,
                &state,
                &mut view,
                &mut scroll,
                &mut interaction,
                24
            )
            .is_none()
        );
    }

    #[test]
    fn refresh_changes_queue_without_redirecting_an_existing_draft() {
        let initial = state(&[
            ("A", AgentStatus::WaitingForInput),
            ("B", AgentStatus::Complete),
        ]);
        let mut interaction = interaction::Interaction::default();
        interaction.sync(initial.items_for(ui::View::Attention));
        interaction.begin_reply(initial.items_for(ui::View::Attention));
        let next = state(&[("A", AgentStatus::Running), ("B", AgentStatus::Complete)]);
        interaction.sync(next.items_for(ui::View::Attention));
        assert_eq!(interaction.selected_id.as_deref(), Some("B"));
        assert_eq!(interaction.draft.as_ref().unwrap().item_id, "A");
        let empty = state(&[("A", AgentStatus::Running), ("B", AgentStatus::Unknown)]);
        interaction.sync(empty.items_for(ui::View::Attention));
        assert!(interaction.selected_id.is_none());
        assert_eq!(interaction.draft.as_ref().unwrap().item_id, "A");
        assert_eq!(empty.items().len(), 2);
    }

    #[test]
    fn empty_or_failed_queue_never_opens_or_replies_to_hidden_work() {
        let empty = state(&[("hidden", AgentStatus::Unknown)]);
        let failed = AppState::from_refresh(
            Err("tmux unavailable".into()),
            Err("state unreadable".into()),
        );
        for state in [empty, failed] {
            let mut view = ui::View::Attention;
            let mut scroll = 0;
            let mut interaction = interaction::Interaction {
                selected_id: Some("hidden".into()),
                ..Default::default()
            };
            assert!(
                navigate(
                    KeyCode::Enter,
                    &state,
                    &mut view,
                    &mut scroll,
                    &mut interaction,
                    24
                )
                .is_none()
            );
            navigate(
                KeyCode::Char('r'),
                &state,
                &mut view,
                &mut scroll,
                &mut interaction,
                24,
            );
            assert!(interaction.draft.is_none());
            assert!(state.attention.is_empty());
            navigate(
                KeyCode::Char('d'),
                &state,
                &mut view,
                &mut scroll,
                &mut interaction,
                24,
            );
            assert!(interaction.review_requested.is_none());
        }
    }
}
